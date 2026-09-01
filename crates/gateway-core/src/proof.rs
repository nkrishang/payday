//! Proof of Payment (product plan §5.4–5.5): an offline-verifiable record
//! tying a canonical invoice to its CREATE3 address, to the transfers that
//! paid it, and to the transaction that settled it. Everything except the
//! verification attestation is recomputable from the proof alone; the
//! attestation is Payday-signed because verification outcomes happen after
//! issuance and cannot be committed into the address.
//!
//! Offline limits: the hash → salt → address chain, the attachment, the
//! attestation, the transfer recipients and the transfer total are all
//! checkable from the proof. Whether the transfers happened and whether the
//! settlement transaction really forwarded the funds is provable only
//! against the chain (`payday proof verify --rpc-url`).

use std::str::FromStr;

use alloy_primitives::{Address, B256, Signature, U256, hex, keccak256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    Amount, AttributionError, BeneficiaryAddress, CANONICALIZATION, CanonicalIssuanceSnapshot,
    FactoryAddress, RecoveryAddress, SNAPSHOT_SCHEMA, Salt, TokenAddress, attribution_hash,
    canonical_bytes, predict_payment_address, recompute_salt,
};

pub const PROOF_VERSION: &str = "payday.proof.v1";
pub const ATTESTATION_VERSION: &str = "payday.attestation.v1";
pub const ATTESTATION_DOMAIN: &[u8] = b"PAYDAY_VERIFICATION_ATTESTATION_V1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTransfer {
    pub transaction_hash: String,
    pub sender: String,
    pub recipient: String,
    pub amount_base_units: String,
    pub block_number: String,
}

/// What Payday signs about a verification outcome (`payday.attestation.v1`:
/// `{version, payment_id, attribution_hash, chain_id, payment_address,
/// payer_policy_mode, result, verified_at}`).
///
/// The issuance commitment is part of the payload because the payment id is
/// a free-form string a proof holder can set to anything: without the
/// attribution hash, chain and CREATE3 address in the signed bytes, a genuine
/// attestation for one invoice would vouch for any fabricated invoice that
/// reuses its id. `verify_proof` recomputes all three from the snapshot and
/// requires them to agree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAttestationPayload {
    pub version: String,
    pub payment_id: String,
    /// `0x` hex, 32 bytes: the attribution hash of the canonical invoice.
    pub attribution_hash: String,
    /// Decimal, as in the canonical issuance snapshot.
    pub chain_id: String,
    /// EIP-55 checksummed CREATE3 payment address.
    pub payment_address: String,
    pub payer_policy_mode: String,
    pub result: String,
    pub verified_at: Option<String>,
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
    pub attribution_nonce: String,
    pub attribution_hash: String,
    pub salt: String,
    pub chain_id: String,
    pub factory_address: String,
    pub payment_address: String,
    pub token_address: String,
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
    #[error("attribution hash does not match the canonical invoice")]
    AttributionHashMismatch,
    #[error("salt is not derived from the nonce and attribution hash")]
    SaltMismatch,
    #[error("chain parameters disagree with the canonical invoice")]
    ChainParametersMismatch,
    #[error("payment address is not the CREATE3 address of the canonical invoice")]
    PaymentAddressMismatch,
    #[error("the invoice commits to no attachment")]
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
    #[error("verification attestation is for a different payment")]
    AttestationPaymentIdMismatch,
    #[error("verification attestation is for a different payer policy mode")]
    AttestationModeMismatch,
    #[error(
        "verification attestation commits to a different invoice (attribution hash, chain, or payment address)"
    )]
    AttestationCommitmentMismatch,
    #[error("a transfer in the proof was not sent to the payment address")]
    TransferRecipientMismatch,
    #[error("the proof's transfers add up to less than the invoice amount")]
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

    // 1. Canonical invoice -> attribution hash.
    let hash = attribution_hash(&canonical_bytes(snapshot)?);
    if hash != word("attribution_hash", &proof.attribution_hash)? {
        return Err(ProofError::AttributionHashMismatch);
    }

    // 2. Nonce and hash -> salt.
    let nonce = word("attribution_nonce", &proof.attribution_nonce)?;
    let salt = recompute_salt(nonce, hash);
    if salt.0 != word("salt", &proof.salt)? {
        return Err(ProofError::SaltMismatch);
    }

    // 3. Salt and the committed terms -> CREATE3 address.
    let factory = FactoryAddress(address("factory_address", &snapshot.factory_address)?);
    let token = TokenAddress(address("token_address", &snapshot.token_address)?);
    if proof.chain_id != snapshot.chain_id
        || address("factory_address", &proof.factory_address)? != factory.0
        || address("token_address", &proof.token_address)? != token.0
    {
        return Err(ProofError::ChainParametersMismatch);
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
        RecoveryAddress(address("recovery_address", &snapshot.recovery_address)?),
        salt,
    )
    .0;
    if payment_address != address("payment_address", &proof.payment_address)? {
        return Err(ProofError::PaymentAddressMismatch);
    }

    // 4. The attachment, when the holder has it.
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

    // 5. The Payday-attested verification outcome.
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
    // The signed commitment must be the one recomputed from the snapshot in
    // steps 1–3, or the attestation was issued for some other invoice.
    if word(
        "verification.payload.attribution_hash",
        &attestation.payload.attribution_hash,
    )? != hash
        || address(
            "verification.payload.payment_address",
            &attestation.payload.payment_address,
        )? != payment_address
        || attestation.payload.chain_id != snapshot.chain_id
    {
        return Err(ProofError::AttestationCommitmentMismatch);
    }

    // 6. The transfers that paid the address, and the transaction that
    // settled it. Amounts and hashes are unsigned data, so offline the checks
    // are structural: every transfer went to the payment address, together
    // they cover the invoice amount, and the settlement hash is well formed.
    word(
        "settlement_transaction_hash",
        &proof.settlement_transaction_hash,
    )?;
    let mut total = U256::ZERO;
    for transfer in &proof.transfers {
        word("transfer transaction_hash", &transfer.transaction_hash)?;
        if address("transfer recipient", &transfer.recipient)? != payment_address {
            return Err(ProofError::TransferRecipientMismatch);
        }
        let credited = U256::from_str_radix(&transfer.amount_base_units, 10)
            .map_err(|_| ProofError::Malformed("transfer amount_base_units"))?;
        total = total
            .checked_add(credited)
            .ok_or(ProofError::Malformed("transfer amount_base_units"))?;
    }
    if total < amount {
        return Err(ProofError::TransfersBelowInvoiceAmount);
    }

    Ok(ProofVerification {
        attribution_hash: hash,
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
        AttachmentCommitment, ChainId, Invoice, Party, PayerPolicy, PayerPolicyMode,
        derive_attribution,
    };

    const ATTACHMENT: &[u8] = b"%PDF-1.7 minimal test document";

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
        issued_for("Globex", 2_500_000, "alice@example.com")
    }

    fn issued_for(bill_to: &str, amount_base_units: u64, expected_email: &str) -> Invoice {
        let factory = FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"));
        let chain_id = ChainId(143);
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(amount_base_units));
        let recovery = RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"));
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
            recovery,
        );
        let digest: [u8; 32] = Sha256::digest(ATTACHMENT).into();
        snapshot.attachment = Some(AttachmentCommitment {
            id: uuid::Uuid::from_u128(9),
            byte_length: ATTACHMENT.len().to_string(),
            sha256: hex::encode_prefixed(digest),
        });
        Invoice::issue(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
            snapshot,
        )
        .unwrap()
    }

    fn proof(invoice: &Invoice) -> ProofOfPayment {
        // The fulfilment transaction is distinct from the payer's transfers.
        let settlement = B256::repeat_byte(0xB2);
        ProofOfPayment {
            version: PROOF_VERSION.into(),
            payment_id: invoice.id.to_string(),
            canonical_issuance_snapshot: invoice.issuance_snapshot.clone(),
            canonicalization: CANONICALIZATION.into(),
            attribution_nonce: invoice.attribution_nonce.to_string(),
            attribution_hash: invoice.attribution_hash.to_string(),
            salt: invoice.salt.0.to_string(),
            chain_id: invoice.chain_id.0.to_string(),
            factory_address: invoice.factory.0.to_checksum(None),
            payment_address: invoice.payment_address.0.to_checksum(None),
            token_address: invoice.token.0.to_checksum(None),
            settlement_transaction_hash: settlement.to_string(),
            transfers: vec![
                ProofTransfer {
                    transaction_hash: B256::repeat_byte(0xA1).to_string(),
                    sender: address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")
                        .to_checksum(None),
                    recipient: invoice.payment_address.0.to_checksum(None),
                    amount_base_units: "1000000".into(),
                    block_number: "3".into(),
                },
                ProofTransfer {
                    transaction_hash: B256::repeat_byte(0xA2).to_string(),
                    sender: address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")
                        .to_checksum(None),
                    recipient: invoice.payment_address.0.to_checksum(None),
                    amount_base_units: "1500000".into(),
                    block_number: "7".into(),
                },
            ],
            verification: sign(VerificationAttestationPayload {
                version: ATTESTATION_VERSION.into(),
                payment_id: invoice.id.to_string(),
                attribution_hash: invoice.attribution_hash.to_string(),
                chain_id: invoice.chain_id.0.to_string(),
                payment_address: invoice.payment_address.0.to_checksum(None),
                payer_policy_mode: PayerPolicyMode::VerifiedEmail.as_str().into(),
                result: "approved".into(),
                verified_at: Some("2026-09-01T12:00:00Z".into()),
            }),
        }
    }

    #[test]
    fn proof_reconstructs_salt_and_create3_address() {
        let invoice = issued();
        let proof = proof(&invoice);
        let verified = verify_proof(&proof, Some(ATTACHMENT), &[attestor()]).unwrap();
        assert_eq!(verified.attribution_hash, invoice.attribution_hash);
        assert_eq!(verified.salt, invoice.salt);
        assert_eq!(verified.payment_address, invoice.payment_address.0);
        assert_eq!(verified.attestation_signer, attestor());
        assert!(verified.attachment_verified);

        // The proof stands on its own: nothing in it came from the database.
        let recomputed = derive_attribution(&proof.canonical_issuance_snapshot).unwrap();
        assert_eq!(recomputed.attribution_hash, verified.attribution_hash);
        assert_eq!(
            recompute_salt(invoice.attribution_nonce, recomputed.attribution_hash),
            verified.salt
        );

        // Without the attachment the proof still verifies, and an empty
        // trust list accepts any self-consistent attestor.
        let without = verify_proof(&proof, None, &[]).unwrap();
        assert!(!without.attachment_verified);

        // A JSON round trip (as `payday proof download` writes it) is lossless.
        let json = serde_json::to_string(&proof).unwrap();
        let parsed: ProofOfPayment = serde_json::from_str(&json).unwrap();
        assert_eq!(
            verify_proof(&parsed, Some(ATTACHMENT), &[attestor()]).unwrap(),
            verified
        );
    }

    #[test]
    fn tampered_snapshot_attachment_or_attestation_fails() {
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
        assert!(matches!(
            check(
                |p| p.attribution_nonce = B256::repeat_byte(0x01).to_string(),
                None
            ),
            ProofError::SaltMismatch
        ));
        assert!(matches!(
            check(|p| p.salt = B256::repeat_byte(0x02).to_string(), None),
            ProofError::SaltMismatch
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
            // Dropping the commitment also changes the canonical invoice.
            ProofError::AttributionHashMismatch
        ));

        assert!(matches!(
            check(|p| p.verification.payload.result = "declined".into(), None),
            ProofError::AttestationSignerMismatch
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
        // 1,000,000 + 1,499,999 falls one base unit short of the invoice.
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
            check(|p| p.version = "payday.proof.v0".into(), None),
            ProofError::Unsupported("proof version")
        ));
    }

    #[test]
    fn a_genuine_attestation_cannot_be_transplanted_onto_another_invoice() {
        // Invoice A is small and real; its attestation is genuine. Invoice B
        // is a fabricated 250,000 USDC invoice to someone else, self-consistent
        // in every recomputable respect, that borrows A's id and attestation.
        let real = issued();
        let genuine = proof(&real).verification;
        let fabricated = issued_for("Initech", 250_000_000_000, "mallory@example.com");
        let mut transplanted = proof(&fabricated);
        transplanted.payment_id = real.id.to_string();
        transplanted.verification = genuine;
        transplanted.transfers[1].amount_base_units = "249999000000".into();
        assert_ne!(real.attribution_hash, fabricated.attribution_hash);

        assert!(matches!(
            verify_proof(&transplanted, None, &[attestor()]).unwrap_err(),
            ProofError::AttestationCommitmentMismatch
        ));

        // Same invoice terms and nonce elsewhere: the chain id alone is
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
