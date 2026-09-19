//! Proof of Payment: an offline-verifiable record tying a canonical deposit
//! request to the CREATE3 address it commits to, to the transfers that paid
//! it, and to the transaction that settled it. Everything except the
//! verification attestation is recomputable from the proof alone; the
//! attestation is Gum-signed because the verification facts (a proven
//! mailbox, a released client secret, and the order in which the steps
//! happened) cannot be committed into the address.
//!
//! ```text
//! attribution_hash = keccak256("GUM_ATTRIBUTION_V5" || JCS(snapshot))
//! salt             = keccak256("GUM_SALT_V5" || issuance_nonce || attribution_hash [+ digest])
//! network          = snapshot.networks[chain_id]
//! payment_address  = CREATE3(network.factory, network.token, amount, receiver, deadline,
//!                             snapshot.recovery_address, salt, network.chain_id)
//! ```
//!
//! The `+ digest` term applies only in the `wallet_attributed` scope, where
//! `digest` is the EIP-712 signing hash of the payer's attestation; in the
//! `settlement` scope the proof makes no claim about who paid, and the salt
//! derives from the issuance nonce and the document alone.
//!
//! The request commits to one currency and to every network it may be paid
//! on, each being that currency's contract on its chain, and to its recovery
//! term: always Gum's own recovery wallet, stated in the snapshot and in
//! the proof as `recovery_address`. A verifier accepts the proof only if the
//! proof's chain is one the request offered and its factory and token are
//! that network's.
//!
//! Scopes. A request that attached wallet attestation proves
//! `wallet_attributed`: the payer's wallet signed the request under the
//! chosen network's domain, and every credited funding transfer came from
//! that wallet — its own address, or Relay's solver for a cross-chain
//! payment the wallet itself sent, vouched for verbatim in the signed
//! attestation. A request without wallet attestation proves `settlement`
//! only: the document, the address, the transfers, and the settlement, with
//! no claim about whose wallet sent anything, because none was made. The
//! scope is committed to the signed attestation, so a holder cannot strip
//! the attestation from a wallet-attributed proof and pass it off as a
//! settlement proof of a different kind — the verifier recomputes the
//! required scope from the snapshot and refuses the mismatch.
//!
//! Offline limits: the hash → salt → address chain, the attachment, Gum's
//! attestation, and the transfers' recipient and total are all checkable
//! from the proof. Whether the transfers happened and whether the settlement
//! transaction really forwarded the funds is provable only against the
//! chain, by fetching the receipts the proof names; a relayed transfer's
//! origin is provable only against its origin chain.

use std::collections::HashSet;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Signature, U256, hex, keccak256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    Amount, AttributionError, BeneficiaryAddress, CANONICALIZATION, CanonicalIssuanceSnapshot,
    ChainId, PayerAttestationError, PayerAttestationScope, PayerWalletAttestation, RecoveryAddress,
    SNAPSHOT_SCHEMA, Salt, attribution_hash, canonical_bytes, predict_payment_address,
    recompute_salt, verify_payer_attestation,
};

pub const PROOF_VERSION: &str = "gum.proof.v5";
pub const ATTESTATION_VERSION: &str = "gum.attestation.v5";
pub const ATTESTATION_DOMAIN: &[u8] = b"GUM_VERIFICATION_ATTESTATION_V5";

/// What the proof stands behind. Committed to the snapshot (a request with
/// the wallet-attestation add-on can only prove `wallet_attributed`, one
/// without it only `settlement`) and to Gum's signed attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofScope {
    /// The request, its address, its credited transfers, and its settlement —
    /// no claim about whose wallet paid.
    Settlement,
    /// The settlement claim, plus: the attested wallet signed this request,
    /// and every credited funding transfer came from that wallet.
    WalletAttributed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTransfer {
    pub transaction_hash: String,
    pub log_index: String,
    pub sender: String,
    pub recipient: String,
    pub amount_base_units: String,
    pub block_number: String,
    /// Present when Relay's solver made this transfer for a cross-chain
    /// payment the attested wallet sent; see [`RelayAttribution`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<RelayAttribution>,
}

/// Where a relayed transfer's funds came from. Not checkable from the
/// destination chain; accepted offline only when the same block appears in
/// the signed attestation's `relay_fills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayAttribution {
    /// Relay's request id, `0x` hex, 32 bytes.
    pub request_id: String,
    /// Decimal chain id the wallet paid on.
    pub origin_chain_id: String,
    /// The transaction the wallet sent there, `0x` hex, 32 bytes.
    pub origin_transaction_hash: String,
    /// EIP-55 checksummed: always the attested wallet.
    pub origin_sender: String,
    /// Always `receipt`: Gum read the origin payment from a chain it
    /// serves. A proof whose attribution names any other source — Relay's
    /// record of a depositor, say — is not a proof the attested wallet
    /// spent the funds, and the verifier refuses it.
    pub attribution_source: String,
}

/// A relayed transfer as the attestation vouches for it: the destination
/// transfer it applies to, and the attribution it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedRelayFill {
    pub transaction_hash: String,
    pub log_index: String,
    #[serde(flatten)]
    pub relay: RelayAttribution,
}

/// One fact Gum observed in the payer's session, with the provider that
/// established it. `mailbox` is the expected email proven by a one-time
/// code; `merchant_session` is the merchant's client secret being exchanged;
/// `wallet` is the attestation signature being accepted. Never a name, an
/// email, or any payer data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationFact {
    /// `mailbox`, `merchant_session`, or `wallet`.
    pub kind: String,
    /// `auth0` for the mailbox code, `merchant` for the client secret,
    /// `gum` for the wallet attestation.
    pub provider: String,
    /// RFC 3339.
    pub at: String,
}

/// What Gum signs about a verification outcome (`gum.attestation.v5`).
///
/// The issuance commitment is part of the payload because the payment id is
/// a free-form string a proof holder can set to anything: without the
/// attribution hash, nonce, chain, and CREATE3 address in the signed bytes,
/// a genuine attestation for one request would vouch for any fabricated one
/// that reuses its id. `verify_proof` recomputes all of them from the
/// snapshot and requires them to agree.
///
/// `wallet_nonce` is the one-time challenge the payer signed; Gum's word
/// is that it was issued to the session only after its identity add-ons
/// passed, which is what orders the identity facts before the wallet
/// signature. It is present exactly when the scope is `wallet_attributed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAttestationPayload {
    pub version: String,
    pub payment_id: String,
    /// The scope this attestation was signed for: the verifier refuses an
    /// attestation carried by a proof of the other scope.
    pub scope: ProofScope,
    /// `0x` hex, 32 bytes: the attribution hash of the canonical request.
    pub attribution_hash: String,
    /// `0x` hex, 32 bytes: the issuance nonce the salt was derived from.
    pub issuance_nonce: String,
    /// Decimal: the chain the payer chose, one of the snapshot's networks.
    pub chain_id: String,
    /// EIP-55 checksummed CREATE3 payment address.
    pub payment_address: String,
    /// EIP-55 checksummed wallet the payer attested; `wallet_attributed`
    /// scope only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer_wallet: Option<String>,
    /// `0x` hex, 32 bytes: the nonce inside the payer's signed attestation;
    /// `wallet_attributed` scope only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_nonce: Option<String>,
    /// `approved` when the attached identity add-ons passed, `not_required`
    /// when none are attached.
    pub result: String,
    pub verified_at: Option<String>,
    /// When the wallet attestation was accepted and the address derived;
    /// `wallet_attributed` scope only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_bound_at: Option<String>,
    pub facts: Vec<VerificationFact>,
    /// The transfers Relay's solver made for cross-chain payments the
    /// attested wallet sent, each with the origin Gum verified. Empty
    /// when every transfer came from the wallet itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_fills: Vec<AttestedRelayFill>,
}

/// `signature` is the 65-byte `r || s || v` secp256k1 signature over
/// [`attestation_digest`], `0x`-prefixed hex; `signer` is the checksummed
/// address the signature recovers to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedVerificationAttestation {
    pub payload: VerificationAttestationPayload,
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfPayment {
    pub version: String,
    pub payment_id: String,
    pub scope: ProofScope,
    pub canonical_issuance_snapshot: CanonicalIssuanceSnapshot,
    pub canonicalization: String,
    pub attribution_hash: String,
    /// `0x` hex, 32 bytes: the issuance nonce, disclosed here so the salt is
    /// recomputable offline. It was kept undisclosed until the payment
    /// address was registered, which is what kept the address underivable —
    /// and so unprefundable — before the service watched for its funds.
    pub issuance_nonce: String,
    /// The payer's wallet attestation: the exact typed data the wallet
    /// signed, its EIP-712 digest, and the signature. Present only in the
    /// `wallet_attributed` scope; the salt below is then derived from the
    /// attribution hash and this digest too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer_wallet: Option<PayerWalletAttestation>,
    pub salt: String,
    /// Decimal: the network the payer chose among `canonical_issuance_snapshot.networks`.
    pub chain_id: String,
    /// That network's factory and token, restated so the derivation reads as
    /// the factory computes it.
    pub factory_address: String,
    pub payment_address: String,
    pub token_address: String,
    /// Always the snapshot's recovery term — Gum's recovery wallet —
    /// stated so the address derivation reads as the factory computes it.
    pub recovery_address: String,
    /// The fulfilment transaction: the `PaymentFactory.execute` call (Gum's
    /// batch sweep or a third party's) that drained the payment address to
    /// the receiver. It is not one of `transfers`, which are the deposits
    /// into the address. Offline, only its shape is checked; that it
    /// forwarded the funds is provable only against the chain.
    pub settlement_transaction_hash: String,
    pub transfers: Vec<ProofTransfer>,
    pub verification: SignedVerificationAttestation,
}

/// What a successful verification established. The wallet fields are `None`
/// exactly when the scope is `settlement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofVerification {
    pub attribution_hash: B256,
    pub scope: ProofScope,
    pub payer_wallet: Option<Address>,
    pub attestation_digest: Option<B256>,
    pub salt: Salt,
    pub payment_address: Address,
    pub attestation_signer: Address,
    /// True when attachment bytes were supplied and matched the commitment.
    pub attachment_verified: bool,
}

#[derive(Debug, Error)]
pub enum ProofError {
    #[error("unsupported {0}")]
    Unsupported(&'static str),
    #[error("{0} is malformed")]
    Malformed(&'static str),
    #[error(transparent)]
    Canonicalization(#[from] AttributionError),
    #[error("attribution hash does not match the canonical request")]
    AttributionHashMismatch,
    #[error(transparent)]
    PayerAttestation(#[from] PayerAttestationError),
    #[error(
        "salt is not derived from the issuance nonce, the attribution hash, and the payer attestation"
    )]
    SaltMismatch,
    #[error("chain parameters disagree with the canonical request")]
    ChainParametersMismatch,
    #[error("the request does not offer the proof's chain")]
    ChainNotOffered,
    #[error("the proof's scope does not match the request's verification add-ons")]
    ScopeMismatch,
    #[error("recovery address is not the issuance snapshot's recovery term")]
    RecoveryAddressMismatch,
    #[error("deposit address is not the CREATE3 address of the request and attestation")]
    PaymentAddressMismatch,
    #[error("the request commits to no attachment")]
    AttachmentNotCommitted,
    #[error("attachment length does not match the commitment")]
    AttachmentLengthMismatch,
    #[error("attachment SHA-256 does not match the commitment")]
    AttachmentHashMismatch,
    #[error("verification attestation signature is invalid")]
    AttestationSignatureInvalid,
    #[error("verification attestation was not signed by its stated signer")]
    AttestationSignerMismatch,
    #[error("verification attestation signer is not a trusted attestor")]
    UntrustedAttestor,
    #[error("verification attestation is for a different deposit request")]
    AttestationPaymentIdMismatch,
    #[error("verification attestation is for a different proof scope")]
    AttestationScopeMismatch,
    #[error("verification attestation does not record a passed outcome")]
    AttestationNotPassed,
    #[error(
        "verification attestation commits to a different request (attribution hash, nonce, chain, deposit address, wallet, or attestation nonce)"
    )]
    AttestationCommitmentMismatch,
    #[error("verification attestation does not record the wallet fact")]
    AttestationWalletFactMissing,
    #[error("a transfer in the proof was not sent to the deposit address")]
    TransferRecipientMismatch,
    #[error("a transfer in the proof was not sent from the payer's attested wallet")]
    TransferSenderMismatch,
    #[error("a relayed transfer's attribution is not vouched for by the verification attestation")]
    RelayFillNotAttested,
    #[error("a relayed transfer's attribution was not verified from the origin chain's receipt")]
    RelayOriginNotVerified,
    #[error("the proof's transfers add up to less than the request amount")]
    TransfersBelowInvoiceAmount,
}

/// `keccak256(ATTESTATION_DOMAIN || JCS(payload))`: what the attestation key
/// signs and what a verifier recovers the signer from.
pub fn attestation_digest(
    payload: &VerificationAttestationPayload,
) -> Result<B256, AttributionError> {
    let canonical = serde_jcs::to_vec(payload)?;
    Ok(keccak256(
        [ATTESTATION_DOMAIN, canonical.as_slice()].concat(),
    ))
}

/// Verify a proof offline. `attachment` is the PDF's bytes when the holder
/// has them; `trusted_attestors`, when non-empty, is the set of Gum
/// attestation keys the holder accepts.
pub fn verify_proof(
    proof: &ProofOfPayment,
    attachment: Option<&[u8]>,
    trusted_attestors: &[Address],
) -> Result<ProofVerification, ProofError> {
    let snapshot = &proof.canonical_issuance_snapshot;
    if proof.version != PROOF_VERSION {
        return Err(ProofError::Unsupported("proof version"));
    }
    if proof.canonicalization != CANONICALIZATION || snapshot.canonicalization != CANONICALIZATION {
        return Err(ProofError::Unsupported("canonicalization"));
    }
    if snapshot.schema != SNAPSHOT_SCHEMA {
        return Err(ProofError::Unsupported("invoice schema"));
    }

    // 0. The scope is not the holder's choice: the request's own add-ons
    //    decide what a proof of it can claim.
    let scope = if snapshot.payer_verification.wallet_attestation {
        ProofScope::WalletAttributed
    } else {
        ProofScope::Settlement
    };
    if proof.scope != scope {
        return Err(ProofError::ScopeMismatch);
    }

    // 1. Canonical request -> attribution hash.
    let hash = attribution_hash(&canonical_bytes(snapshot)?);
    if hash != word("attribution_hash", &proof.attribution_hash)? {
        return Err(ProofError::AttributionHashMismatch);
    }
    let nonce = word("issuance_nonce", &proof.issuance_nonce)?;

    // 2. The network the payer chose must be one the request offered, and
    // the proof's factory and token must be that network's.
    let chain_id: u64 = proof
        .chain_id
        .parse()
        .map_err(|_| ProofError::Malformed("chain_id"))?;
    let networks = snapshot
        .networks()
        .ok_or(ProofError::Malformed("networks"))?;
    let network = *networks
        .iter()
        .find(|network| network.chain_id == ChainId(chain_id))
        .ok_or(ProofError::ChainNotOffered)?;
    if address("factory_address", &proof.factory_address)? != network.factory.0
        || address("token_address", &proof.token_address)? != network.token.0
    {
        return Err(ProofError::ChainParametersMismatch);
    }
    let (factory, token) = (network.factory, network.token);

    // 3. The wallet attestation, in the wallet-attributed scope only: made
    // for this request on that chain's factory, and signed by the wallet it
    // names. A settlement proof claims nothing about any wallet and carries
    // none.
    let attested = match (scope, &proof.payer_wallet) {
        (ProofScope::WalletAttributed, Some(attestation)) => Some(verify_payer_attestation(
            attestation,
            PayerAttestationScope {
                chain_id,
                factory: factory.0,
                attribution_hash: hash,
            },
        )?),
        (ProofScope::WalletAttributed, None) => return Err(ProofError::Malformed("payer_wallet")),
        (ProofScope::Settlement, None) => None,
        (ProofScope::Settlement, Some(_)) => {
            return Err(ProofError::Malformed("payer_wallet"));
        }
    };

    // 4. Nonce, hash, and (when attested) the attestation digest -> salt.
    let salt = recompute_salt(
        nonce,
        hash,
        attested.as_ref().map(|attested| attested.digest),
    );
    if salt.0 != word("salt", &proof.salt)? {
        return Err(ProofError::SaltMismatch);
    }

    // 5. Salt, the chosen network, the committed terms, and the snapshot's
    // recovery term -> CREATE3 address.
    let recovery = RecoveryAddress(address("recovery_address", &proof.recovery_address)?);
    if recovery.0 != address("recovery_address", &snapshot.recovery_address)? {
        return Err(ProofError::RecoveryAddressMismatch);
    }
    let amount = U256::from_str_radix(&snapshot.amount_base_units, 10)
        .map_err(|_| ProofError::Malformed("amount_base_units"))?;
    let payment_address = predict_payment_address(
        factory,
        token,
        Amount(amount),
        BeneficiaryAddress(address("receiver_address", &snapshot.receiver_address)?),
        snapshot
            .expiration_timestamp
            .parse()
            .map_err(|_| ProofError::Malformed("expiration_timestamp"))?,
        recovery,
        salt,
        network.chain_id,
    )
    .0;
    if payment_address != address("payment_address", &proof.payment_address)? {
        return Err(ProofError::PaymentAddressMismatch);
    }

    // 6. The attachment, when the holder has it.
    let attachment_verified = match attachment {
        None => false,
        Some(bytes) => {
            let commitment = snapshot
                .attachment
                .as_ref()
                .ok_or(ProofError::AttachmentNotCommitted)?;
            if commitment.byte_length != bytes.len().to_string() {
                return Err(ProofError::AttachmentLengthMismatch);
            }
            let digest: [u8; 32] = Sha256::digest(bytes).into();
            if B256::from(digest) != word("attachment sha256", &commitment.sha256)? {
                return Err(ProofError::AttachmentHashMismatch);
            }
            true
        }
    };

    // 7. The Gum-attested verification outcome.
    let attestation = &proof.verification;
    let digest = attestation_digest(&attestation.payload)?;
    let signature = hex::decode(&attestation.signature)
        .ok()
        .and_then(|bytes| Signature::from_raw(&bytes).ok())
        .ok_or(ProofError::AttestationSignatureInvalid)?;
    let recovered = signature
        .recover_address_from_prehash(&digest)
        .map_err(|_| ProofError::AttestationSignatureInvalid)?;
    if recovered != address("verification.signer", &attestation.signer)? {
        return Err(ProofError::AttestationSignerMismatch);
    }
    if !trusted_attestors.is_empty() && !trusted_attestors.contains(&recovered) {
        return Err(ProofError::UntrustedAttestor);
    }
    if attestation.payload.version != ATTESTATION_VERSION {
        return Err(ProofError::Unsupported("attestation version"));
    }
    if attestation.payload.payment_id != proof.payment_id {
        return Err(ProofError::AttestationPaymentIdMismatch);
    }
    if attestation.payload.scope != scope {
        return Err(ProofError::AttestationScopeMismatch);
    }
    let passed_result = if snapshot.payer_verification.is_gated() {
        "approved"
    } else {
        "not_required"
    };
    if attestation.payload.result != passed_result {
        return Err(ProofError::AttestationNotPassed);
    }
    // The signed commitment must be the one recomputed from the snapshot and
    // the attestation in steps 1–5, or the attestation was issued for some
    // other request.
    let wallet_matches = match (&attestation.payload.payer_wallet, &attested) {
        (Some(payload_wallet), Some(attested)) => {
            address("verification.payload.payer_wallet", payload_wallet)? == attested.wallet
                && word(
                    "verification.payload.wallet_nonce",
                    attestation
                        .payload
                        .wallet_nonce
                        .as_deref()
                        .ok_or(ProofError::AttestationCommitmentMismatch)?,
                )? == attested.nonce
        }
        (None, None) => true,
        _ => return Err(ProofError::AttestationCommitmentMismatch),
    };
    if !wallet_matches
        || word(
            "verification.payload.attribution_hash",
            &attestation.payload.attribution_hash,
        )? != hash
        || word(
            "verification.payload.issuance_nonce",
            &attestation.payload.issuance_nonce,
        )? != nonce
        || address(
            "verification.payload.payment_address",
            &attestation.payload.payment_address,
        )? != payment_address
        || attestation.payload.chain_id != proof.chain_id
    {
        return Err(ProofError::AttestationCommitmentMismatch);
    }
    if scope == ProofScope::WalletAttributed
        && !attestation
            .payload
            .facts
            .iter()
            .any(|fact| fact.kind == "wallet")
    {
        return Err(ProofError::AttestationWalletFactMissing);
    }

    // 8. The transfers that paid the address, and the transaction that
    // settled it. Amounts and hashes are unsigned data, so offline the checks
    // are structural: every transfer went to the payment address and
    // together they cover the request amount; in the wallet-attributed
    // scope, every one also came from the attested wallet, directly or
    // through a relayed fill the attestation vouches for.
    word(
        "settlement_transaction_hash",
        &proof.settlement_transaction_hash,
    )?;
    let mut total = U256::ZERO;
    // Canonical event identity prevents textual/metadata aliases inflating it.
    let mut unique_transfers = HashSet::new();
    for transfer in &proof.transfers {
        let hash = word("transfer transaction_hash", &transfer.transaction_hash)?;
        let log_index = transfer
            .log_index
            .parse::<u64>()
            .map_err(|_| ProofError::Malformed("transfer log_index"))?;
        if address("transfer recipient", &transfer.recipient)? != payment_address {
            return Err(ProofError::TransferRecipientMismatch);
        }
        if scope == ProofScope::WalletAttributed {
            let wallet = attested.as_ref().expect("checked above").wallet;
            if address("transfer sender", &transfer.sender)? != wallet {
                // Not the wallet's own transfer: acceptable only as a relayed
                // one the attestation vouches for, whose origin sender is the
                // wallet. The block must match the attestation's verbatim, so
                // nothing in it can be edited after signing.
                let relay = transfer
                    .relay
                    .as_ref()
                    .ok_or(ProofError::TransferSenderMismatch)?;
                if relay.attribution_source != "receipt" {
                    // The attestation is where Gum vouches that it verified
                    // who spent the funds from the origin chain itself; anything
                    // else is Relay's or the page's word, which is no evidence.
                    return Err(ProofError::RelayOriginNotVerified);
                }
                word("transfer relay.request_id", &relay.request_id)?;
                word(
                    "transfer relay.origin_transaction_hash",
                    &relay.origin_transaction_hash,
                )?;
                relay
                    .origin_chain_id
                    .parse::<u64>()
                    .map_err(|_| ProofError::Malformed("transfer relay.origin_chain_id"))?;
                if address("transfer relay.origin_sender", &relay.origin_sender)? != wallet {
                    return Err(ProofError::TransferSenderMismatch);
                }
                let vouched = proof.verification.payload.relay_fills.iter().any(|fill| {
                    fill.transaction_hash == transfer.transaction_hash
                        && fill.log_index == transfer.log_index
                        && fill.relay == *relay
                });
                if !vouched {
                    return Err(ProofError::RelayFillNotAttested);
                }
            }
        } else if let Some(relay) = &transfer.relay {
            // A settlement proof may carry a relayed transfer's provenance,
            // but it vouches for nothing: only the shape is checked.
            word("transfer relay.request_id", &relay.request_id)?;
            word(
                "transfer relay.origin_transaction_hash",
                &relay.origin_transaction_hash,
            )?;
            relay
                .origin_chain_id
                .parse::<u64>()
                .map_err(|_| ProofError::Malformed("transfer relay.origin_chain_id"))?;
            address("transfer relay.origin_sender", &relay.origin_sender)?;
            if relay.attribution_source.is_empty() {
                return Err(ProofError::Malformed("transfer relay.attribution_source"));
            }
        }
        let credited = U256::from_str_radix(&transfer.amount_base_units, 10)
            .map_err(|_| ProofError::Malformed("transfer amount_base_units"))?;
        transfer
            .block_number
            .parse::<u64>()
            .map_err(|_| ProofError::Malformed("transfer block_number"))?;
        if unique_transfers.insert((hash, log_index)) {
            total = total
                .checked_add(credited)
                .ok_or(ProofError::Malformed("transfer amount_base_units"))?;
        }
    }
    if total < amount {
        return Err(ProofError::TransfersBelowInvoiceAmount);
    }

    Ok(ProofVerification {
        attribution_hash: hash,
        scope,
        payer_wallet: attested.as_ref().map(|attested| attested.wallet),
        attestation_digest: attested.as_ref().map(|attested| attested.digest),
        salt,
        payment_address,
        attestation_signer: recovered,
        attachment_verified,
    })
}

fn word(field: &'static str, value: &str) -> Result<B256, ProofError> {
    B256::from_str(value).map_err(|_| ProofError::Malformed(field))
}

fn address(field: &'static str, value: &str) -> Result<Address, ProofError> {
    Address::from_str(value).map_err(|_| ProofError::Malformed(field))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;
    use k256::ecdsa::SigningKey;

    use super::*;
    use crate::{
        AttachmentCommitment, Currency, EmailVerification, FactoryAddress, Invoice, NetworkTerms,
        Party, PayerAttestation, PayerVerification, PaymentBinding, TokenAddress,
        derive_attribution, sign_payer_attestation, wallet_of,
    };

    const MONAD: ChainId = ChainId(143);
    const BASE: ChainId = ChainId(8453);
    /// The snapshot's recovery term in tests: Gum's recovery wallet.
    const RECOVERY: Address = address!("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955");

    fn networks() -> Vec<NetworkTerms> {
        vec![
            NetworkTerms {
                chain_id: MONAD,
                token: TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
                factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            },
            NetworkTerms {
                chain_id: BASE,
                token: TokenAddress(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
                factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            },
        ]
    }

    const ATTACHMENT: &[u8] = b"%PDF-1.7 minimal test document";
    const PAYER_KEY: [u8; 32] = [3u8; 32];
    const OTHER_PAYER_KEY: [u8; 32] = [4u8; 32];

    fn signing_key() -> SigningKey {
        SigningKey::from_slice(&[7u8; 32]).unwrap()
    }

    fn attestor() -> Address {
        Address::from_public_key(signing_key().verifying_key())
    }

    fn sign(payload: VerificationAttestationPayload) -> SignedVerificationAttestation {
        let digest = attestation_digest(&payload).unwrap();
        let (signature, recovery) = signing_key()
            .sign_prehash_recoverable(digest.as_slice())
            .unwrap();
        let signature = Signature::from_signature_and_parity(signature, recovery.is_y_odd());
        SignedVerificationAttestation {
            payload,
            signer: attestor().to_checksum(None),
            signature: hex::encode_prefixed(signature.as_bytes()),
        }
    }

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    fn issued() -> Invoice {
        issued_for(
            Verification::Email,
            "Globex",
            2_500_000,
            "alice@example.com",
            &PAYER_KEY,
            MONAD,
        )
    }

    /// Which add-ons the sample request carries.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Verification {
        /// Email verification plus wallet attestation, as the old
        /// `verified_email` mode was.
        Email,
        /// No add-ons at all: the default, fully permissionless request.
        None,
    }

    fn issued_for(
        verification: Verification,
        bill_to: &str,
        amount_base_units: u64,
        expected_email: &str,
        payer_key: &[u8; 32],
        chain_id: ChainId,
    ) -> Invoice {
        let payer_verification = match verification {
            Verification::Email => PayerVerification {
                email: Some(EmailVerification {
                    expected_email: expected_email.into(),
                }),
                merchant_auth: None,
                wallet_attestation: true,
            },
            Verification::None => PayerVerification::default(),
        };
        let networks = networks();
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(amount_base_units));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party(bill_to),
            payer_verification,
            Currency::Usdc,
            &networks,
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            1_900_000_000,
        );
        let digest: [u8; 32] = Sha256::digest(ATTACHMENT).into();
        snapshot.attachment = Some(AttachmentCommitment {
            id: uuid::Uuid::from_u128(9),
            byte_length: ATTACHMENT.len().to_string(),
            sha256: hex::encode_prefixed(digest),
        });
        let mut invoice = Invoice::issue(
            Currency::Usdc,
            &networks,
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            1_900_000_000,
            snapshot,
        )
        .unwrap();
        // The Email sample binds through the wallet attestation; the
        // permissionless one through the address-only network binding.
        match verification {
            Verification::Email => {
                let message = PayerAttestation::new(
                    invoice.attribution_hash,
                    wallet_of(payer_key),
                    B256::repeat_byte(0x11),
                    1_900_000_000,
                );
                let factory = invoice.network_for(chain_id).unwrap().factory;
                let attestation =
                    sign_payer_attestation(payer_key, &message, chain_id.0, factory.0);
                let binding = invoice
                    .bind_payer_wallet(chain_id, attestation, "2026-09-01T11:59:00Z".into())
                    .unwrap();
                invoice.binding = Some(binding);
            }
            Verification::None => {
                let binding = invoice
                    .bind_network(chain_id, "2026-09-01T11:59:00Z".into())
                    .unwrap();
                invoice.binding = Some(binding);
            }
        }
        invoice
    }

    fn binding(invoice: &Invoice) -> &PaymentBinding {
        invoice.binding.as_ref().unwrap()
    }

    fn payload(invoice: &Invoice) -> VerificationAttestationPayload {
        let binding = binding(invoice);
        let attested = invoice
            .issuance_snapshot
            .payer_verification
            .wallet_attestation;
        VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            scope: if attested {
                ProofScope::WalletAttributed
            } else {
                ProofScope::Settlement
            },
            attribution_hash: invoice.attribution_hash.to_string(),
            issuance_nonce: invoice.issuance_nonce.to_string(),
            chain_id: binding.network.chain_id.to_string(),
            payment_address: binding.payment_address.0.to_checksum(None),
            payer_wallet: attested.then(|| {
                binding
                    .wallet
                    .as_ref()
                    .unwrap()
                    .payer_wallet
                    .to_checksum(None)
            }),
            wallet_nonce: attested.then(|| {
                binding
                    .wallet
                    .as_ref()
                    .unwrap()
                    .attestation
                    .typed_data
                    .message
                    .nonce
                    .clone()
            }),
            result: if invoice.issuance_snapshot.payer_verification.is_gated() {
                "approved"
            } else {
                "not_required"
            }
            .into(),
            verified_at: Some("2026-09-01T11:58:00Z".into()),
            wallet_bound_at: attested.then(|| binding.wallet.as_ref().unwrap().bound_at.clone()),
            facts: if attested {
                vec![
                    VerificationFact {
                        kind: "mailbox".into(),
                        provider: "auth0".into(),
                        at: "2026-09-01T11:58:00Z".into(),
                    },
                    VerificationFact {
                        kind: "wallet".into(),
                        provider: "gum".into(),
                        at: binding.ready_at.clone(),
                    },
                ]
            } else {
                vec![]
            },
            relay_fills: Vec::new(),
        }
    }

    fn proof(invoice: &Invoice) -> ProofOfPayment {
        let binding = binding(invoice);
        let attested = invoice
            .issuance_snapshot
            .payer_verification
            .wallet_attestation;
        // The fulfilment transaction is distinct from the payer's transfers.
        let settlement = B256::repeat_byte(0xB2);
        let sender = if attested {
            binding
                .wallet
                .as_ref()
                .unwrap()
                .payer_wallet
                .to_checksum(None)
        } else {
            // A permissionless request is paid from any wallet; here, an
            // exchange's.
            wallet_of(&OTHER_PAYER_KEY).to_checksum(None)
        };
        let transfer = |byte: u8, log_index: &str, amount: &str, block: &str| ProofTransfer {
            transaction_hash: B256::repeat_byte(byte).to_string(),
            log_index: log_index.into(),
            sender: sender.clone(),
            recipient: binding.payment_address.0.to_checksum(None),
            amount_base_units: amount.into(),
            block_number: block.into(),
            relay: None,
        };
        ProofOfPayment {
            version: PROOF_VERSION.into(),
            payment_id: invoice.id.to_string(),
            scope: if attested {
                ProofScope::WalletAttributed
            } else {
                ProofScope::Settlement
            },
            canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
            canonicalization: CANONICALIZATION.into(),
            attribution_hash: invoice.attribution_hash.to_string(),
            issuance_nonce: invoice.issuance_nonce.to_string(),
            payer_wallet: attested.then(|| binding.wallet.as_ref().unwrap().attestation.clone()),
            salt: binding.salt.0.to_string(),
            chain_id: binding.network.chain_id.to_string(),
            factory_address: binding.network.factory.0.to_checksum(None),
            payment_address: binding.payment_address.0.to_checksum(None),
            token_address: binding.network.token.0.to_checksum(None),
            recovery_address: binding.recovery.0.to_checksum(None),
            settlement_transaction_hash: settlement.to_string(),
            transfers: vec![
                transfer(0xA1, "0", "1000000", "3"),
                transfer(0xA2, "1", "1500000", "7"),
            ],
            verification: sign(payload(invoice)),
        }
    }

    #[test]
    fn a_wallet_attributed_proof_reconstructs_attestation_salt_and_create3_address() {
        let invoice = issued();
        let proof = proof(&invoice);
        let verified = verify_proof(&proof, Some(ATTACHMENT), &[attestor()]).unwrap();
        let binding = binding(&invoice);
        assert_eq!(verified.scope, ProofScope::WalletAttributed);
        assert_eq!(verified.attribution_hash, invoice.attribution_hash);
        assert_eq!(verified.payer_wallet, Some(wallet_of(&PAYER_KEY)));
        assert_eq!(
            verified.attestation_digest.unwrap().to_string(),
            binding.wallet.as_ref().unwrap().attestation.digest
        );
        assert_eq!(verified.salt, binding.salt);
        assert_eq!(verified.payment_address, binding.payment_address.0);
        assert_eq!(verified.attestation_signer, attestor());
        assert!(verified.attachment_verified);

        // The proof stands on its own: nothing in it came from the database.
        let recomputed = derive_attribution(&proof.canonical_issuance_snapshot).unwrap();
        assert_eq!(recomputed.attribution_hash, verified.attribution_hash);
        assert_eq!(
            recompute_salt(
                invoice.issuance_nonce,
                recomputed.attribution_hash,
                verified.attestation_digest
            ),
            verified.salt
        );

        // Without the attachment the proof still verifies, and an empty
        // trust list accepts any self-consistent attestor.
        let without = verify_proof(&proof, None, &[]).unwrap();
        assert!(!without.attachment_verified);

        // A JSON round trip (as `GET /v1/deposit-requests/{id}/proof` serves it) is lossless.
        let json = serde_json::to_string(&proof).unwrap();
        let parsed: ProofOfPayment = serde_json::from_str(&json).unwrap();
        assert_eq!(
            verify_proof(&parsed, Some(ATTACHMENT), &[attestor()]).unwrap(),
            verified
        );
    }

    #[test]
    fn a_settlement_proof_proves_the_payment_without_claiming_a_wallet() {
        let invoice = issued_for(
            Verification::None,
            "Globex",
            2_500_000,
            "",
            &PAYER_KEY,
            MONAD,
        );
        let proof = proof(&invoice);
        assert!(proof.payer_wallet.is_none());
        let verified = verify_proof(&proof, Some(ATTACHMENT), &[attestor()]).unwrap();
        assert_eq!(verified.scope, ProofScope::Settlement);
        assert_eq!(verified.payer_wallet, None);
        assert_eq!(verified.attestation_digest, None);
        assert_eq!(
            verified.payment_address,
            binding(&invoice).payment_address.0
        );
        assert!(verified.attachment_verified);

        // The transfers may come from anywhere at all — an exchange, a
        // Relay solver, two different wallets at once.
        let mut mixed = proof.clone();
        mixed.transfers[0].sender = wallet_of(&PAYER_KEY).to_checksum(None);
        assert_eq!(
            verify_proof(&mixed, Some(ATTACHMENT), &[]).unwrap(),
            verified
        );

        // The scope cannot be upgraded, downgraded, or stripped: the
        // snapshot's add-ons decide it.
        let mut upgraded = proof.clone();
        upgraded.scope = ProofScope::WalletAttributed;
        assert!(matches!(
            verify_proof(&upgraded, None, &[]).unwrap_err(),
            ProofError::ScopeMismatch
        ));
        // Carrying an attestation block in a settlement proof is refused:
        // the block binds another request's attribution hash, and in any
        // case a settlement proof claims nothing about any wallet.
        let attested_invoice = issued();
        let mut carrying = proof.clone();
        carrying.payer_wallet = Some(
            binding(&attested_invoice)
                .wallet
                .as_ref()
                .unwrap()
                .attestation
                .clone(),
        );
        assert!(verify_proof(&carrying, None, &[]).is_err());
    }

    #[test]
    fn tampered_snapshot_attestation_wallet_or_transfers_fail() {
        let invoice = issued();
        let good = proof(&invoice);
        let check = |mutate: fn(&mut ProofOfPayment), attachment: Option<&[u8]>| {
            let mut proof = good.clone();
            mutate(&mut proof);
            verify_proof(&proof, attachment, &[attestor()]).unwrap_err()
        };

        assert!(matches!(
            check(
                |p| p.canonical_issuance_snapshot.notes = Some("edited".into()),
                None
            ),
            ProofError::AttributionHashMismatch
        ));
        assert!(matches!(
            check(
                |p| p.canonical_issuance_snapshot.amount_base_units = "2500001".into(),
                None
            ),
            ProofError::AttributionHashMismatch
        ));
        // The payer attestation is bound to the request and to its wallet.
        assert!(matches!(
            check(
                |p| p.payer_wallet.as_mut().unwrap().typed_data.message.nonce =
                    B256::repeat_byte(0x12).to_string(),
                None
            ),
            ProofError::PayerAttestation(PayerAttestationError::DigestMismatch)
        ));
        assert!(matches!(
            check(
                |p| p.payer_wallet.as_mut().unwrap().signature = "0x1234".into(),
                None
            ),
            ProofError::PayerAttestation(PayerAttestationError::SignatureInvalid)
        ));
        assert!(matches!(
            check(|p| p.salt = B256::repeat_byte(0x02).to_string(), None),
            ProofError::SaltMismatch
        ));
        // The nonce is committed: swapping it moves the salt.
        assert!(matches!(
            check(
                |p| p.issuance_nonce = B256::repeat_byte(0x31).to_string(),
                None
            ),
            ProofError::SaltMismatch
        ));
        // The recovery term is the snapshot's, and the proof cannot move it
        // — not even to the attested wallet.
        assert!(matches!(
            check(
                |p| p.recovery_address = Address::repeat_byte(0x03).to_checksum(None),
                None
            ),
            ProofError::RecoveryAddressMismatch
        ));
        assert!(matches!(
            check(
                |p| p.recovery_address = wallet_of(&PAYER_KEY).to_checksum(None),
                None
            ),
            ProofError::RecoveryAddressMismatch
        ));
        assert!(matches!(
            check(
                |p| p.payment_address = Address::repeat_byte(0x03).to_checksum(None),
                None
            ),
            ProofError::PaymentAddressMismatch
        ));
        // The chain must be one the request offered, and the factory and
        // token must be that network's.
        assert!(matches!(
            check(|p| p.chain_id = "1".into(), None),
            ProofError::ChainNotOffered
        ));
        assert!(matches!(
            check(|p| p.chain_id = "8453".into(), None),
            ProofError::ChainParametersMismatch
        ));
        assert!(matches!(
            check(
                |p| p.token_address = Address::repeat_byte(0x08).to_checksum(None),
                None
            ),
            ProofError::ChainParametersMismatch
        ));
        // Restating Base's token with Base's chain id moves the attestation
        // domain: the signature was made for Monad.
        assert!(matches!(
            check(
                |p| {
                    p.chain_id = "8453".into();
                    p.token_address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into();
                },
                None
            ),
            ProofError::PayerAttestation(PayerAttestationError::DomainMismatch)
        ));

        let mut longer = ATTACHMENT.to_vec();
        longer.push(b'\n');
        assert!(matches!(
            verify_proof(&good, Some(&longer), &[]).unwrap_err(),
            ProofError::AttachmentLengthMismatch
        ));
        let mut altered = ATTACHMENT.to_vec();
        altered[0] ^= 0xFF;
        assert!(matches!(
            verify_proof(&good, Some(&altered), &[]).unwrap_err(),
            ProofError::AttachmentHashMismatch
        ));
        assert!(matches!(
            check(
                |p| p.canonical_issuance_snapshot.attachment = None,
                Some(ATTACHMENT)
            ),
            // Dropping the commitment also changes the canonical request.
            ProofError::AttributionHashMismatch
        ));

        assert!(matches!(
            check(|p| p.verification.payload.result = "declined".into(), None),
            ProofError::AttestationSignerMismatch
        ));
        let mut pending = good.clone();
        pending.verification.payload.result = "pending".into();
        pending.verification = sign(pending.verification.payload);
        assert!(matches!(
            verify_proof(&pending, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationNotPassed
        ));
        let mut no_wallet_fact = good.clone();
        no_wallet_fact
            .verification
            .payload
            .facts
            .retain(|fact| fact.kind != "wallet");
        no_wallet_fact.verification = sign(no_wallet_fact.verification.payload);
        assert!(matches!(
            verify_proof(&no_wallet_fact, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationWalletFactMissing
        ));
        assert!(matches!(
            check(
                |p| p.verification.signer = Address::repeat_byte(0x04).to_checksum(None),
                None
            ),
            ProofError::AttestationSignerMismatch
        ));
        assert!(matches!(
            check(|p| p.verification.signature = "0x1234".into(), None),
            ProofError::AttestationSignatureInvalid
        ));
        assert!(matches!(
            verify_proof(&good, None, &[Address::repeat_byte(0x05)]).unwrap_err(),
            ProofError::UntrustedAttestor
        ));
        let other = issued();
        let mut foreign = good.clone();
        foreign.verification = proof(&other).verification;
        assert!(matches!(
            verify_proof(&foreign, None, &[]).unwrap_err(),
            ProofError::AttestationPaymentIdMismatch
        ));
        // An attestation signed for the other scope is refused even when it
        // names this request's id: the payload is signed with the settlement
        // invoice's commitments and its scope.
        let wrong_scope = {
            let settlement_invoice = issued_for(
                Verification::None,
                "Globex",
                2_500_000,
                "",
                &PAYER_KEY,
                MONAD,
            );
            let mut payload = payload(&settlement_invoice);
            payload.payment_id = good.payment_id.clone();
            let mut p = proof(&settlement_invoice);
            p.payment_id = good.payment_id.clone();
            p.verification = sign(payload);
            p.verification
        };
        let mut carried = good.clone();
        carried.verification = wrong_scope;
        if let Err(error) = verify_proof(&carried, None, &[]) {
            assert!(
                matches!(error, ProofError::AttestationScopeMismatch),
                "carried attestation failed with {error:?}"
            );
        } else {
            panic!("carried attestation verified");
        }
        // Editing the signed commitment breaks the signature before anything
        // else is compared.
        assert!(matches!(
            check(
                |p| p.verification.payload.attribution_hash = B256::repeat_byte(0x07).to_string(),
                None
            ),
            ProofError::AttestationSignerMismatch
        ));

        assert!(matches!(
            check(
                |p| p.transfers[0].recipient = Address::repeat_byte(0x06).to_checksum(None),
                None
            ),
            ProofError::TransferRecipientMismatch
        ));
        // Money from any wallet but the attested one is not the payer's.
        assert!(matches!(
            check(
                |p| p.transfers[1].sender = wallet_of(&OTHER_PAYER_KEY).to_checksum(None),
                None
            ),
            ProofError::TransferSenderMismatch
        ));
        // Unless Relay's solver sent it for the wallet, and the attestation
        // vouches for exactly that attribution.
        let solver = Address::repeat_byte(0x50);
        let relay = |origin_sender: Address| RelayAttribution {
            request_id: B256::repeat_byte(0x71).to_string(),
            origin_chain_id: "8453".into(),
            origin_transaction_hash: B256::repeat_byte(0x72).to_string(),
            origin_sender: origin_sender.to_checksum(None),
            attribution_source: "receipt".into(),
        };
        let relayed = |origin_sender: Address, vouch: bool| {
            let mut proof = proof(&invoice);
            proof.transfers[1].sender = solver.to_checksum(None);
            proof.transfers[1].relay = Some(relay(origin_sender));
            let mut payload = payload(&invoice);
            if vouch {
                payload.relay_fills.push(AttestedRelayFill {
                    transaction_hash: proof.transfers[1].transaction_hash.clone(),
                    log_index: proof.transfers[1].log_index.clone(),
                    relay: relay(origin_sender),
                });
            }
            proof.verification = sign(payload);
            proof
        };
        let wallet = wallet_of(&PAYER_KEY);
        assert!(verify_proof(&relayed(wallet, true), None, &[]).is_ok());
        assert!(matches!(
            verify_proof(&relayed(wallet, false), None, &[]).unwrap_err(),
            ProofError::RelayFillNotAttested
        ));
        // The attestation vouching for a relayed transfer from another
        // wallet does not make it the payer's.
        assert!(matches!(
            verify_proof(&relayed(wallet_of(&OTHER_PAYER_KEY), true), None, &[]).unwrap_err(),
            ProofError::TransferSenderMismatch
        ));
        // Editing the transfer's block after signing breaks the match.
        let mut edited = relayed(wallet, true);
        edited.transfers[1].relay.as_mut().unwrap().origin_chain_id = "42161".into();
        assert!(matches!(
            verify_proof(&edited, None, &[]).unwrap_err(),
            ProofError::RelayFillNotAttested
        ));
        // A relayed transfer without the block is just a foreign transfer.
        let mut bare = relayed(wallet, true);
        bare.transfers[1].relay = None;
        assert!(matches!(
            verify_proof(&bare, None, &[]).unwrap_err(),
            ProofError::TransferSenderMismatch
        ));
        // An attribution from any source but a verified receipt — Relay's
        // record of a depositor, say — is refused even when the attestation
        // vouches for it: the vouch is only ever as good as the evidence.
        let mut unverified = relayed(wallet, true);
        let mut relay = unverified.transfers[1].relay.clone().unwrap();
        relay.attribution_source = "relay_api".into();
        let vouched_relay = relay.clone();
        unverified.transfers[1].relay = Some(relay);
        unverified.verification.payload.relay_fills[0].relay = vouched_relay;
        unverified.verification = sign(unverified.verification.payload);
        assert!(matches!(
            verify_proof(&unverified, None, &[]).unwrap_err(),
            ProofError::RelayOriginNotVerified
        ));
        // 1,000,000 + 1,499,999 falls one base unit short of the request.
        assert!(matches!(
            check(
                |p| p.transfers[1].amount_base_units = "1499999".into(),
                None
            ),
            ProofError::TransfersBelowInvoiceAmount
        ));
        assert!(matches!(
            check(|p| p.transfers.clear(), None),
            ProofError::TransfersBelowInvoiceAmount
        ));
        let mut duplicated = good.clone();
        duplicated.transfers.truncate(1);
        duplicated.transfers.push(duplicated.transfers[0].clone());
        assert!(matches!(
            verify_proof(&duplicated, None, &[]).unwrap_err(),
            ProofError::TransfersBelowInvoiceAmount
        ));
        assert!(matches!(
            check(|p| p.transfers[0].amount_base_units = "1e6".into(), None),
            ProofError::Malformed("transfer amount_base_units")
        ));
        // A settlement hash the proof does not list among the transfers is
        // the normal case (it is the fulfilment call); only its shape is
        // checked offline.
        assert!(
            verify_proof(
                &{
                    let mut proof = good.clone();
                    proof.settlement_transaction_hash = B256::repeat_byte(0xC3).to_string();
                    proof
                },
                None,
                &[]
            )
            .is_ok()
        );
        assert!(matches!(
            check(|p| p.settlement_transaction_hash = "0xc3".into(), None),
            ProofError::Malformed("settlement_transaction_hash")
        ));
        assert!(matches!(
            check(|p| p.version = "gum.proof.v1".into(), None),
            ProofError::Unsupported("proof version")
        ));
    }

    #[test]
    fn a_genuine_attestation_cannot_be_transplanted_onto_another_request_or_payer() {
        // Request A is small and real; its attestation is genuine. Request B
        // is a fabricated 250,000 USDC request to someone else, self-consistent
        // in every recomputable respect, that borrows A's id and attestation.
        let real = issued();
        let genuine = proof(&real).verification;
        let fabricated = issued_for(
            Verification::Email,
            "Initech",
            250_000_000_000,
            "mallory@example.com",
            &PAYER_KEY,
            MONAD,
        );
        let mut transplanted = proof(&fabricated);
        transplanted.payment_id = real.id.to_string();
        transplanted.verification = genuine.clone();
        transplanted.transfers[1].amount_base_units = "249999000000".into();
        assert_ne!(real.attribution_hash, fabricated.attribution_hash);
        assert!(matches!(
            verify_proof(&transplanted, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));

        // Same request text, a different payer: the wallet and nonce in the
        // signed payload no longer match the attestation in the proof, and
        // neither does the address.
        let other_payer = issued_for(
            Verification::Email,
            "Globex",
            2_500_000,
            "alice@example.com",
            &OTHER_PAYER_KEY,
            MONAD,
        );
        assert_eq!(real.attribution_hash, other_payer.attribution_hash);
        let mut swapped = proof(&other_payer);
        swapped.payment_id = real.id.to_string();
        swapped.verification = genuine;
        assert!(matches!(
            verify_proof(&swapped, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));

        // Same request terms and attestation elsewhere: the chain id alone is
        // enough to reject the attestation.
        let mut other_chain = proof(&real);
        other_chain.verification.payload.chain_id = "8453".into();
        other_chain.verification = sign(other_chain.verification.payload);
        assert!(matches!(
            verify_proof(&other_chain, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));

        // Swapping the disclosed issuance nonce breaks the attestation's
        // commitment before the salt check can even matter.
        let mut other_nonce = proof(&real);
        other_nonce.verification.payload.issuance_nonce = B256::repeat_byte(0x33).to_string();
        other_nonce.verification = sign(other_nonce.verification.payload);
        assert!(matches!(
            verify_proof(&other_nonce, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));
    }

    #[test]
    fn a_request_paid_on_another_of_its_networks_proves_that_network() {
        // The same request text, paid on Base: the attestation domain, the
        // token, and the address all follow the payer's choice, and the
        // proof verifies against the Base entry of the snapshot.
        let on_base = issued_for(
            Verification::Email,
            "Globex",
            2_500_000,
            "alice@example.com",
            &PAYER_KEY,
            BASE,
        );
        let proof = proof(&on_base);
        assert_eq!(proof.chain_id, "8453");
        assert_eq!(
            proof.token_address,
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        );
        assert_eq!(
            proof
                .payer_wallet
                .as_ref()
                .unwrap()
                .typed_data
                .domain
                .chain_id,
            8453
        );
        let verified = verify_proof(&proof, Some(ATTACHMENT), &[attestor()]).unwrap();
        assert_eq!(
            verified.payment_address,
            binding(&on_base).payment_address.0
        );
        let on_monad = issued();
        assert_eq!(on_monad.attribution_hash, on_base.attribution_hash);
        assert_ne!(
            binding(&on_monad).payment_address,
            binding(&on_base).payment_address
        );
    }

    #[test]
    fn a_settlement_proof_cannot_inherit_another_request_s_identity_facts() {
        // A permissionless request whose attestation payload was signed with
        // the identity facts of a gated one: the scope the snapshot demands
        // is settlement, the attestation must not claim a wallet fact, and
        // the commitments must still bind it to this request alone.
        let invoice = issued_for(
            Verification::None,
            "Globex",
            2_500_000,
            "",
            &PAYER_KEY,
            MONAD,
        );
        let mut proof = proof(&invoice);
        proof.verification.payload.facts.push(VerificationFact {
            kind: "mailbox".into(),
            provider: "auth0".into(),
            at: "2026-09-01T11:58:00Z".into(),
        });
        proof.verification = sign(proof.verification.payload);
        let verified = verify_proof(&proof, None, &[]).unwrap();
        assert_eq!(verified.scope, ProofScope::Settlement);
        assert_eq!(verified.payer_wallet, None);
    }
}
