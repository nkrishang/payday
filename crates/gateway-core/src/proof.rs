//! Proof of Payment (product plan §5.4–5.5): an offline-verifiable record
//! tying a canonical deposit request to the wallet its payer attested, to the
//! CREATE3 address both commit to, to the transfers from that wallet that
//! paid it, and to the transaction that settled it. Everything except the
//! verification attestation is recomputable from the proof alone; the
//! attestation is Payday-signed because the identity facts (a proven
//! mailbox, and the order in which the steps happened) cannot be committed
//! into the address.
//!
//! ```text
//! attribution_hash = keccak256("PAYDAY_ATTRIBUTION_V2" || JCS(snapshot))
//! digest           = EIP-712 signing hash of payer_wallet.typed_data
//! salt             = keccak256("PAYDAY_SALT_V2" || attribution_hash || digest)
//! payment_address  = CREATE3(factory, token, amount, receiver, deadline, payer_wallet, salt)
//! ```
//!
//! Offline limits: the hash → attestation → salt → address chain, the
//! attachment, Payday's attestation, and the transfers' recipient, sender
//! and total are all checkable from the proof. Whether the transfers
//! happened and whether the settlement transaction really forwarded the
//! funds is provable only against the chain, by fetching the receipts the
//! proof names.

use std::collections::HashSet;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Signature, U256, hex, keccak256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    Amount, AttributionError, BeneficiaryAddress, CANONICALIZATION, CanonicalIssuanceSnapshot,
    FactoryAddress, PayerAttestationError, PayerAttestationScope, PayerWalletAttestation,
    RecoveryAddress, SNAPSHOT_SCHEMA, Salt, TokenAddress, attribution_hash, canonical_bytes,
    predict_payment_address, recompute_salt, verify_payer_attestation,
};

pub const PROOF_VERSION: &str = "payday.proof.v2";
pub const ATTESTATION_VERSION: &str = "payday.attestation.v2";
pub const ATTESTATION_DOMAIN: &[u8] = b"PAYDAY_VERIFICATION_ATTESTATION_V2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTransfer {
    pub transaction_hash: String,
    pub log_index: String,
    pub sender: String,
    pub recipient: String,
    pub amount_base_units: String,
    pub block_number: String,
}

/// One fact Payday observed in the payer's session, with the provider that
/// established it. `mailbox` is the expected email proven by a one-time
/// code; `merchant_session` is the merchant's client secret being exchanged;
/// `wallet` is the attestation signature being accepted. Never a name, an
/// email, or any payer data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationFact {
    /// `mailbox` or `wallet`.
    pub kind: String,
    /// `auth0` for the mailbox code, `payday` for the wallet attestation.
    pub provider: String,
    /// RFC 3339.
    pub at: String,
}

/// What Payday signs about a verification outcome (`payday.attestation.v2`).
///
/// The issuance commitment is part of the payload because the payment id is
/// a free-form string a proof holder can set to anything: without the
/// attribution hash, chain, CREATE3 address, and payer wallet in the signed
/// bytes, a genuine attestation for one request would vouch for any
/// fabricated one that reuses its id. `verify_proof` recomputes all of them
/// from the snapshot and the wallet attestation and requires them to agree.
///
/// `wallet_nonce` is the one-time challenge the payer signed; Payday's word
/// is that it was issued to the session only after the policy passed, which
/// is what orders the mailbox fact before the wallet signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAttestationPayload {
    pub version: String,
    pub payment_id: String,
    /// `0x` hex, 32 bytes: the attribution hash of the canonical request.
    pub attribution_hash: String,
    /// Decimal, as in the canonical issuance snapshot.
    pub chain_id: String,
    /// EIP-55 checksummed CREATE3 payment address.
    pub payment_address: String,
    /// EIP-55 checksummed wallet the payer attested.
    pub payer_wallet: String,
    /// `0x` hex, 32 bytes: the nonce inside the payer's signed attestation.
    pub wallet_nonce: String,
    pub payer_policy_mode: String,
    /// `approved` for a gated policy that passed, `not_required` for a
    /// permissionless one.
    pub result: String,
    pub verified_at: Option<String>,
    /// When the wallet attestation was accepted and the address derived.
    pub wallet_bound_at: String,
    pub facts: Vec<VerificationFact>,
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
    pub canonical_issuance_snapshot: CanonicalIssuanceSnapshot,
    pub canonicalization: String,
    pub attribution_hash: String,
    /// The payer's wallet attestation: the exact typed data the wallet
    /// signed, its EIP-712 digest, and the signature. The salt below is
    /// derived from the attribution hash and this digest, and the wallet is
    /// the address's recovery term.
    pub payer_wallet: PayerWalletAttestation,
    pub salt: String,
    pub chain_id: String,
    pub factory_address: String,
    pub payment_address: String,
    pub token_address: String,
    /// Always the payer's attested wallet; stated so the address derivation
    /// reads as the factory computes it.
    pub recovery_address: String,
    /// The fulfilment transaction: the `PaymentFactory.execute` call (Payday's
    /// batch sweep or a third party's) that drained the payment address to
    /// the receiver. It is not one of `transfers`, which are the payer's
    /// USDC transfers into the address. Offline, only its shape is checked;
    /// that it forwarded the funds is provable only against the chain.
    pub settlement_transaction_hash: String,
    pub transfers: Vec<ProofTransfer>,
    pub verification: SignedVerificationAttestation,
}

/// What a successful verification established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofVerification {
    pub attribution_hash: B256,
    pub payer_wallet: Address,
    pub attestation_digest: B256,
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
    #[error("salt is not derived from the attribution hash and the payer attestation")]
    SaltMismatch,
    #[error("chain parameters disagree with the canonical request")]
    ChainParametersMismatch,
    #[error("recovery address is not the payer's attested wallet")]
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
    #[error("verification attestation is for a different payer policy mode")]
    AttestationModeMismatch,
    #[error("verification attestation does not record a passed outcome")]
    AttestationNotPassed,
    #[error(
        "verification attestation commits to a different request (attribution hash, chain, deposit address, wallet, or nonce)"
    )]
    AttestationCommitmentMismatch,
    #[error("verification attestation does not record the wallet fact")]
    AttestationWalletFactMissing,
    #[error("a transfer in the proof was not sent to the deposit address")]
    TransferRecipientMismatch,
    #[error("a transfer in the proof was not sent from the payer's attested wallet")]
    TransferSenderMismatch,
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
/// has them; `trusted_attestors`, when non-empty, is the set of Payday
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

    // 1. Canonical request -> attribution hash.
    let hash = attribution_hash(&canonical_bytes(snapshot)?);
    if hash != word("attribution_hash", &proof.attribution_hash)? {
        return Err(ProofError::AttributionHashMismatch);
    }

    // 2. The payer's wallet attestation: made for this request on this
    // deployment, and signed by the wallet it names.
    let factory = FactoryAddress(address("factory_address", &snapshot.factory_address)?);
    let token = TokenAddress(address("token_address", &snapshot.token_address)?);
    if proof.chain_id != snapshot.chain_id
        || address("factory_address", &proof.factory_address)? != factory.0
        || address("token_address", &proof.token_address)? != token.0
    {
        return Err(ProofError::ChainParametersMismatch);
    }
    let chain_id: u64 = snapshot
        .chain_id
        .parse()
        .map_err(|_| ProofError::Malformed("chain_id"))?;
    let attested = verify_payer_attestation(
        &proof.payer_wallet,
        PayerAttestationScope {
            chain_id,
            factory: factory.0,
            attribution_hash: hash,
        },
    )?;

    // 3. Hash and attestation digest -> salt.
    let salt = recompute_salt(hash, attested.digest);
    if salt.0 != word("salt", &proof.salt)? {
        return Err(ProofError::SaltMismatch);
    }

    // 4. Salt, the committed terms, and the wallet as recovery -> CREATE3
    // address.
    let recovery = RecoveryAddress(address("recovery_address", &proof.recovery_address)?);
    if recovery.0 != attested.wallet {
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
    )
    .0;
    if payment_address != address("payment_address", &proof.payment_address)? {
        return Err(ProofError::PaymentAddressMismatch);
    }

    // 5. The attachment, when the holder has it.
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

    // 6. The Payday-attested verification outcome.
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
    if attestation.payload.payer_policy_mode != snapshot.payer_policy.mode().as_str() {
        return Err(ProofError::AttestationModeMismatch);
    }
    let passed_result = if snapshot.payer_policy.mode().is_gated() {
        "approved"
    } else {
        "not_required"
    };
    if attestation.payload.result != passed_result {
        return Err(ProofError::AttestationNotPassed);
    }
    // The signed commitment must be the one recomputed from the snapshot and
    // the wallet attestation in steps 1–4, or the attestation was issued for
    // some other request or some other payer.
    if word(
        "verification.payload.attribution_hash",
        &attestation.payload.attribution_hash,
    )? != hash
        || address(
            "verification.payload.payment_address",
            &attestation.payload.payment_address,
        )? != payment_address
        || address(
            "verification.payload.payer_wallet",
            &attestation.payload.payer_wallet,
        )? != attested.wallet
        || word(
            "verification.payload.wallet_nonce",
            &attestation.payload.wallet_nonce,
        )? != attested.nonce
        || attestation.payload.chain_id != snapshot.chain_id
    {
        return Err(ProofError::AttestationCommitmentMismatch);
    }
    if !attestation
        .payload
        .facts
        .iter()
        .any(|fact| fact.kind == "wallet")
    {
        return Err(ProofError::AttestationWalletFactMissing);
    }

    // 7. The transfers that paid the address, and the transaction that
    // settled it. Amounts and hashes are unsigned data, so offline the checks
    // are structural: every transfer went to the payment address from the
    // attested wallet, together they cover the request amount, and the
    // settlement hash is well formed.
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
        if address("transfer sender", &transfer.sender)? != attested.wallet {
            return Err(ProofError::TransferSenderMismatch);
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
        payer_wallet: attested.wallet,
        attestation_digest: attested.digest,
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
        AttachmentCommitment, ChainId, Invoice, Party, PayerAttestation, PayerPolicy,
        PayerPolicyMode, PaymentBinding, derive_attribution, sign_payer_attestation, wallet_of,
    };

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
        issued_for("Globex", 2_500_000, "alice@example.com", &PAYER_KEY)
    }

    fn issued_for(
        bill_to: &str,
        amount_base_units: u64,
        expected_email: &str,
        payer_key: &[u8; 32],
    ) -> Invoice {
        let factory = FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"));
        let chain_id = ChainId(143);
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(amount_base_units));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party(bill_to),
            PayerPolicy::VerifiedEmail {
                expected_email: expected_email.into(),
            },
            factory,
            chain_id,
            token,
            beneficiary,
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
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            snapshot,
        )
        .unwrap();
        let message = PayerAttestation::new(
            invoice.attribution_hash,
            wallet_of(payer_key),
            B256::repeat_byte(0x11),
            1_900_000_000,
        );
        let attestation = sign_payer_attestation(payer_key, &message, chain_id.0, factory.0);
        let binding = invoice
            .bind_payer_wallet(attestation, "2026-09-01T11:59:00Z".into())
            .unwrap();
        invoice.binding = Some(binding);
        invoice
    }

    fn binding(invoice: &Invoice) -> &PaymentBinding {
        invoice.binding.as_ref().unwrap()
    }

    fn payload(invoice: &Invoice) -> VerificationAttestationPayload {
        let binding = binding(invoice);
        VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: invoice.id.to_string(),
            attribution_hash: invoice.attribution_hash.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            payment_address: binding.payment_address.0.to_checksum(None),
            payer_wallet: binding.payer_wallet.to_checksum(None),
            wallet_nonce: binding.attestation.typed_data.message.nonce.clone(),
            payer_policy_mode: PayerPolicyMode::VerifiedEmail.as_str().into(),
            result: "approved".into(),
            verified_at: Some("2026-09-01T11:58:00Z".into()),
            wallet_bound_at: binding.bound_at.clone(),
            facts: vec![
                VerificationFact {
                    kind: "mailbox".into(),
                    provider: "auth0".into(),
                    at: "2026-09-01T11:58:00Z".into(),
                },
                VerificationFact {
                    kind: "wallet".into(),
                    provider: "payday".into(),
                    at: binding.bound_at.clone(),
                },
            ],
        }
    }

    fn proof(invoice: &Invoice) -> ProofOfPayment {
        let binding = binding(invoice);
        // The fulfilment transaction is distinct from the payer's transfers.
        let settlement = B256::repeat_byte(0xB2);
        let transfer = |byte: u8, log_index: &str, amount: &str, block: &str| ProofTransfer {
            transaction_hash: B256::repeat_byte(byte).to_string(),
            log_index: log_index.into(),
            sender: binding.payer_wallet.to_checksum(None),
            recipient: binding.payment_address.0.to_checksum(None),
            amount_base_units: amount.into(),
            block_number: block.into(),
        };
        ProofOfPayment {
            version: PROOF_VERSION.into(),
            payment_id: invoice.id.to_string(),
            canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
            canonicalization: CANONICALIZATION.into(),
            attribution_hash: invoice.attribution_hash.to_string(),
            payer_wallet: binding.attestation.clone(),
            salt: binding.salt.0.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            factory_address: invoice.factory.0.to_checksum(None),
            payment_address: binding.payment_address.0.to_checksum(None),
            token_address: invoice.token.0.to_checksum(None),
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
    fn proof_reconstructs_attestation_salt_and_create3_address() {
        let invoice = issued();
        let proof = proof(&invoice);
        let verified = verify_proof(&proof, Some(ATTACHMENT), &[attestor()]).unwrap();
        let binding = binding(&invoice);
        assert_eq!(verified.attribution_hash, invoice.attribution_hash);
        assert_eq!(verified.payer_wallet, wallet_of(&PAYER_KEY));
        assert_eq!(
            verified.attestation_digest.to_string(),
            binding.attestation.digest
        );
        assert_eq!(verified.salt, binding.salt);
        assert_eq!(verified.payment_address, binding.payment_address.0);
        assert_eq!(verified.attestation_signer, attestor());
        assert!(verified.attachment_verified);

        // The proof stands on its own: nothing in it came from the database.
        let recomputed = derive_attribution(&proof.canonical_issuance_snapshot).unwrap();
        assert_eq!(recomputed.attribution_hash, verified.attribution_hash);
        assert_eq!(
            recompute_salt(recomputed.attribution_hash, verified.attestation_digest),
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
                |p| p.payer_wallet.typed_data.message.nonce = B256::repeat_byte(0x12).to_string(),
                None
            ),
            ProofError::PayerAttestation(PayerAttestationError::DigestMismatch)
        ));
        assert!(matches!(
            check(|p| p.payer_wallet.signature = "0x1234".into(), None),
            ProofError::PayerAttestation(PayerAttestationError::SignatureInvalid)
        ));
        assert!(matches!(
            check(|p| p.salt = B256::repeat_byte(0x02).to_string(), None),
            ProofError::SaltMismatch
        ));
        assert!(matches!(
            check(
                |p| p.recovery_address = Address::repeat_byte(0x03).to_checksum(None),
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
        assert!(matches!(
            check(|p| p.chain_id = "1".into(), None),
            ProofError::ChainParametersMismatch
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
            check(|p| p.version = "payday.proof.v1".into(), None),
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
            "Initech",
            250_000_000_000,
            "mallory@example.com",
            &PAYER_KEY,
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
        let other_payer = issued_for("Globex", 2_500_000, "alice@example.com", &OTHER_PAYER_KEY);
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
        other_chain.verification.payload.chain_id = "1".into();
        other_chain.verification = sign(other_chain.verification.payload);
        assert!(matches!(
            verify_proof(&other_chain, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));
    }
}
