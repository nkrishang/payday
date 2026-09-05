//! Proof of Payment attestations (product plan §5.5): Payday's signature over
//! a verification outcome. Outcomes happen after issuance, so they cannot be
//! committed into the payment address; they are signed instead, with a key
//! dedicated to attestations — never the sweep signer or the recovery wallet.

use std::sync::Arc;

use alloy_primitives::{Address, hex};
use alloy_signer::Signer;
use alloy_signer_aws::AwsSigner;
use alloy_signer_local::PrivateKeySigner;
use gateway_core::{
    AttributionError, SignedVerificationAttestation, VerificationAttestationPayload,
    attestation_digest,
};
use thiserror::Error;

use crate::config::AttestationSignerConfig;

/// Mirrors the indexer's signer selection: a raw key locally, KMS in
/// production, chosen by which variable is set.
enum Backend {
    Local(PrivateKeySigner),
    Kms(AwsSigner),
}

#[derive(Clone)]
pub struct VerificationAttestor {
    backend: Arc<Backend>,
}

#[derive(Debug, Error)]
pub enum AttestationError {
    #[error("attestation payload could not be canonicalized: {0}")]
    Canonicalization(#[from] AttributionError),
    #[error("attestation signing failed: {0}")]
    Signing(#[from] alloy_signer::Error),
}

impl VerificationAttestor {
    pub fn local(signer: PrivateKeySigner) -> Self {
        Self {
            backend: Arc::new(Backend::Local(signer)),
        }
    }

    pub async fn from_config(
        config: &AttestationSignerConfig,
        sdk_config: &aws_config::SdkConfig,
    ) -> Result<Self, String> {
        match config {
            AttestationSignerConfig::Local(key) => {
                let signer: PrivateKeySigner = key
                    .parse()
                    .map_err(|error| format!("invalid PAYDAY_ATTESTATION_SIGNER_KEY: {error}"))?;
                Ok(Self::local(signer))
            }
            AttestationSignerConfig::AwsKms(key_id) => {
                let kms = aws_sdk_kms::Client::new(sdk_config);
                // No chain id: attestations are prehash signatures, not
                // transactions, so EIP-155 never applies.
                let signer = AwsSigner::new(kms, key_id.clone(), None)
                    .await
                    .map_err(|error| {
                        format!("failed to initialize PAYDAY_ATTESTATION_KMS_KEY_ID: {error}")
                    })?;
                Ok(Self {
                    backend: Arc::new(Backend::Kms(signer)),
                })
            }
        }
    }

    /// The address verifiers must trust as the attestor (`verify_proof`'s
    /// `trusted_attestors`).
    pub fn address(&self) -> Address {
        match &*self.backend {
            Backend::Local(signer) => signer.address(),
            Backend::Kms(signer) => signer.address(),
        }
    }

    /// Sign `keccak256(ATTESTATION_DOMAIN || JCS(payload))` as a raw prehash
    /// (KMS `MessageType=DIGEST`) and encode it the way `verify_proof` reads it.
    pub async fn attest(
        &self,
        payload: VerificationAttestationPayload,
    ) -> Result<SignedVerificationAttestation, AttestationError> {
        let digest = attestation_digest(&payload)?;
        let signature = match &*self.backend {
            Backend::Local(signer) => signer.sign_hash(&digest).await?,
            Backend::Kms(signer) => signer.sign_hash(&digest).await?,
        };
        Ok(SignedVerificationAttestation {
            payload,
            signer: self.address().to_checksum(None),
            signature: hex::encode_prefixed(signature.as_bytes()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use alloy_primitives::Signature;
    use gateway_core::ATTESTATION_VERSION;

    use super::*;

    fn payload(result: &str) -> VerificationAttestationPayload {
        VerificationAttestationPayload {
            version: ATTESTATION_VERSION.into(),
            payment_id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700".into(),
            attribution_hash: alloy_primitives::B256::repeat_byte(0x11).to_string(),
            chain_id: "143".into(),
            payment_address: Address::repeat_byte(0x22).to_checksum(None),
            payer_wallet: Address::repeat_byte(0x33).to_checksum(None),
            wallet_nonce: alloy_primitives::B256::repeat_byte(0x44).to_string(),
            payer_policy_mode: "permissionless".into(),
            result: result.into(),
            verified_at: None,
            wallet_bound_at: "2026-09-06T00:00:00Z".into(),
            facts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn attestations_recover_to_the_signer_and_change_with_the_payload() {
        let sdk_config = aws_config::SdkConfig::builder().build();
        let attestor = VerificationAttestor::from_config(
            &AttestationSignerConfig::Local(
                "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e".into(),
            ),
            &sdk_config,
        )
        .await
        .unwrap();
        assert_eq!(
            attestor.address(),
            Address::from_str("0x976EA74026E726554dB657fA54763abd0C3a0aa9").unwrap(),
            "Anvil account #6, the trusted attestor of the e2e suite"
        );

        let signed = attestor.attest(payload("not_required")).await.unwrap();
        assert_eq!(signed.signer, attestor.address().to_checksum(None));
        let bytes = hex::decode(&signed.signature).unwrap();
        assert_eq!(bytes.len(), 65);
        let signature = Signature::from_raw(&bytes).unwrap();
        let digest = attestation_digest(&signed.payload).unwrap();
        assert_eq!(
            signature.recover_address_from_prehash(&digest).unwrap(),
            attestor.address()
        );

        let other = attestor.attest(payload("approved")).await.unwrap();
        assert_ne!(other.signature, signed.signature);
        assert_ne!(
            Signature::from_raw(&hex::decode(&other.signature).unwrap())
                .unwrap()
                .recover_address_from_prehash(&digest)
                .unwrap(),
            attestor.address(),
            "a signature over one payload does not vouch for another"
        );
        assert!(
            VerificationAttestor::from_config(
                &AttestationSignerConfig::Local("0x1234".into()),
                &sdk_config,
            )
            .await
            .is_err()
        );
    }
}
